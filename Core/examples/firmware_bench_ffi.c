// Run the exact APK library without Java/AudioTrack or persistent app state.
// Usage: firmware_bench_ffi libfm1android.so package.fwsc [blocks=300] [period=100] [held=75]
#include <assert.h>
#include <dlfcn.h>
#include <inttypes.h>
#include <math.h>
#include <stdbool.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>

static double now(void) { struct timespec t; assert(clock_gettime(CLOCK_MONOTONIC,&t)==0); return t.tv_sec+t.tv_nsec*1e-9; }
static int compare(const void *a,const void *b) { double x=*(const double *)a,y=*(const double *)b; return (x>y)-(x<y); }
int main(int argc,char **argv) {
    assert(argc>=3);
    size_t count=argc>3?strtoul(argv[3],NULL,10):300;
    size_t period=argc>4?strtoul(argv[4],NULL,10):100;
    size_t held=argc>5?strtoul(argv[5],NULL,10):75;
    assert(count>0 && period>held && held>0);
    void *library=dlopen(argv[1],RTLD_NOW|RTLD_LOCAL);
    if(!library) { fprintf(stderr,"%s\n",dlerror()); return 1; }
    void *(*create)(const uint8_t *,size_t,bool,uint8_t *,size_t)=dlsym(library,"instrument_create_binary");
    size_t (*render)(void *,float *,size_t)=dlsym(library,"instrument_render");
    void (*key)(void *,uint32_t,bool)=dlsym(library,"instrument_key");
    void (*destroy)(void *)=dlsym(library,"instrument_destroy");
    assert(create && render && key && destroy);
    FILE *file=fopen(argv[2],"rb"); assert(file);
    assert(fseek(file,0,SEEK_END)==0); long length=ftell(file); assert(length>=960 && length<=4*1024*1024);
    rewind(file); uint8_t *bytes=malloc((size_t)length); assert(bytes);
    assert(fread(bytes,1,(size_t)length,file)==(size_t)length); fclose(file);
    uint8_t error[512]={0}; double start=now(); void *instrument=create(bytes,(size_t)length,true,error,sizeof(error)); free(bytes);
    if(!instrument) { fprintf(stderr,"%s\n",error); return 1; }
    float pcm[1024]; for(size_t i=0;i<200;i++) (void)render(instrument,pcm,512);
    double boot=(now()-start)*1000, *times=calloc(count,sizeof(double)); assert(times);
    uint64_t hash=UINT64_C(0xcbf29ce484222325); size_t frames=0,late=0; float peak=0;
    const uint32_t keys[]={21,25,28,32}; start=now();
    for(size_t i=0;i<count;i++) {
        if(i%period==0) for(size_t k=0;k<4;k++) key(instrument,keys[k],true);
        if(i%period==held) for(size_t k=0;k<4;k++) key(instrument,keys[k],false);
        double begin=now(); size_t n=render(instrument,pcm,512); assert(n==512);
        times[i]=(now()-begin)*1000; late+=times[i]>512.0/44.1; frames+=n;
        for(size_t j=0;j<n*2;j++) { assert(isfinite(pcm[j])); peak=fmaxf(peak,fabsf(pcm[j])); uint32_t bits; memcpy(&bits,&pcm[j],4); hash=(hash^bits)*UINT64_C(0x100000001b3); }
    }
    double elapsed=now()-start; qsort(times,count,sizeof(double),compare);
    printf("{\"blocks\":%zu,\"chordPeriodBlocks\":%zu,\"heldBlocks\":%zu,\"boot_ms\":%.3f,\"frames\":%zu,\"wall_seconds\":%.6f,\"realtime_ratio\":%.4f,\"p50_ms\":%.3f,\"p95_ms\":%.3f,\"max_ms\":%.3f,\"over_11_61_ms\":%zu,\"peak\":%.8f,\"pcm_hash\":\"%016" PRIx64 "\"}\n",
        count,period,held,boot,frames,elapsed,frames/44100.0/elapsed,times[count/2],times[count*95/100],times[count-1],late,peak,hash);
    free(times); destroy(instrument); dlclose(library); return 0;
}
